export type ReceiptScope={plan:string;material:number;unit:number};
export type ReceiptReference={key:number;plan:string;revision:string;material?:number;unit?:number};
type Lookup=ReceiptScope&{revision:string};
const database='omloom-receipt-recovery',storeName='pending';

function transaction<T>(mode:IDBTransactionMode,work:(store:IDBObjectStore,done:(value:T)=>void,fail:(error:unknown)=>void)=>void):Promise<T>{
  return new Promise((resolve,reject)=>{
    let blocked=false;
    const request=indexedDB.open(database,2);
    request.onupgradeneeded=()=>{
      const db=request.result;
      const store=db.objectStoreNames.contains(storeName)?request.transaction!.objectStore(storeName):db.createObjectStore(storeName,{autoIncrement:true});
      store.createIndex('plan','plan');
    };
    request.onerror=()=>reject(request.error);
    request.onblocked=()=>{blocked=true;reject(Error('Close other receipt editors before opening recovery storage.'));};
    request.onsuccess=()=>{
      const db=request.result;
      if(blocked){db.close();return;}
      let tx:IDBTransaction;
      try{tx=db.transaction(storeName,mode,mode==='readwrite'?{durability:'strict'}:undefined);}
      catch(e){db.close();reject(e);return;}
      let result:T,failure:unknown;
      tx.oncomplete=()=>{db.close();resolve(result);};
      tx.onabort=()=>{db.close();reject(failure===undefined?tx.error:failure);};
      try{work(tx.objectStore(storeName),value=>{result=value;},error=>{failure=error;tx.abort();});}
      catch(e){failure=e;tx.abort();}
    };
  });
}

function lookup(value:unknown):Omit<ReceiptReference,'key'>{
  if(typeof value!=='object'||value===null||!('plan'in value)||!('revision'in value)||typeof value.plan!=='string'||typeof value.revision!=='string'||!/^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/.test(value.plan)||!/^\d{1,20}$/.test(value.revision))throw Error('The saved receipt reference is invalid.');
  const material='material'in value?value.material:undefined,unit='unit'in value?value.unit:undefined;
  if(material===undefined&&unit===undefined)return {plan:value.plan,revision:value.revision};
  if(typeof material!=='number'||!Number.isInteger(material)||material<0||material>=64||typeof unit!=='number'||!Number.isInteger(unit)||unit<0||unit>4294967295)throw Error('The saved device selection is invalid.');
  return {plan:value.plan,revision:value.revision,material,unit};
}

function references(store:IDBObjectStore,plan:string,done:(values:ReceiptReference[])=>void,fail:(e:unknown)=>void){
  const request=store.index('plan').openCursor(IDBKeyRange.only(plan)),values:ReceiptReference[]=[];
  request.onsuccess=()=>{
    const cursor=request.result;
    if(!cursor){done(values);return;}
    try{
      if(values.length>=128||typeof cursor.primaryKey!=='number')throw Error('Too many unresolved device saves.');
      values.push({key:cursor.primaryKey,...lookup(cursor.value)});cursor.continue();
    }catch(e){fail(e);}
  };
}
export function matchesReceipt(value:ReceiptReference,scope:ReceiptScope){
  // Earlier references without a recorded unit remain plan-local until recovered.
  return value.plan===scope.plan&&(value.material===undefined||(value.material===scope.material&&value.unit===scope.unit));
}
export function readReceiptReferences(plan:string):Promise<ReceiptReference[]>{
  return transaction('readonly',(store,done,fail)=>references(store,plan,done,fail));
}

export function claimReceiptReference(value:Lookup):Promise<ReceiptReference|undefined>{
  // IndexedDB 2.0 section 2.7: overlapping readwrite transactions execute serially.
  // Only the same material unit conflicts; unrelated receiving remains usable.
  // The generated key fences delayed cross-tab cleanup, including equal revisions.
  return transaction('readwrite',(store,done,fail)=>references(store,value.plan,values=>{
      if(values.some(previous=>matchesReceipt(previous,value))){done(undefined);return;}
      const count=store.count();
      count.onsuccess=()=>{
      if(count.result>=128){fail(Error('Resolve outstanding device saves before storing more recovery references.'));return;}
      const added=store.add(value);
      added.onsuccess=()=>done({key:added.result as number,...value});
      };
  },fail));
}

export function clearReceiptReference(value:ReceiptReference):Promise<void>{
  return transaction('readwrite',(store,done)=>{store.delete(value.key);done(undefined);});
}
